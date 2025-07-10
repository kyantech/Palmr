import type { User } from "@prisma/client";

import { prisma } from "../../shared/prisma";
import type { RegisterUserInput, UpdateUserInput } from "./dto";

export interface IUserRepository {
  createUser(data: RegisterUserInput & { password: string }): Promise<User>;
  findUserByEmail(email: string): Promise<User | null>;
  findUserById(id: string): Promise<User | null>;
  findUserByUsername(username: string): Promise<User | null>;
  findUserByEmailOrUsername(emailOrUsername: string): Promise<User | null>;
  listUsers(): Promise<User[]>;
  updateUser(data: UpdateUserInput & { password?: string }): Promise<User>;
  deleteUser(id: string): Promise<User>;
  activateUser(id: string): Promise<User>;
  deactivateUser(id: string): Promise<User>;
}

export class PrismaUserRepository implements IUserRepository {
  async createUser(data: RegisterUserInput & { password: string }): Promise<User> {
    return prisma.user.create({
      data: {
        firstName: data.firstName,
        lastName: data.lastName,
        username: data.username,
        email: data.email,
        password: data.password,
        image: data.image,
        isAdmin: data.isAdmin,
      },
    });
  }

  async findUserByEmail(email: string): Promise<User | null> {
    return prisma.user.findUnique({ where: { email } });
  }

  async findUserById(id: string): Promise<User | null> {
    return prisma.user.findUnique({ where: { id } });
  }

  async findUserByUsername(username: string): Promise<User | null> {
    return prisma.user.findUnique({ where: { username } });
  }

  async findUserByEmailOrUsername(emailOrUsername: string): Promise<User | null> {
    return prisma.user.findFirst({
      where: {
        OR: [{ email: emailOrUsername }, { username: emailOrUsername }],
      },
    });
  }

  async listUsers(): Promise<User[]> {
    return prisma.user.findMany();
  }

  async updateUser(data: UpdateUserInput & { password?: string }): Promise<User> {
    const { id, ...rest } = data;
    return prisma.user.update({
      where: { id },
      data: rest,
    });
  }

  async deleteUser(id: string): Promise<User> {
    return prisma.user.delete({ where: { id } });
  }

  async activateUser(id: string): Promise<User> {
    return prisma.user.update({
      where: { id },
      data: { isActive: true },
    });
  }

  async deactivateUser(id: string): Promise<User> {
    return prisma.user.update({
      where: { id },
      data: { isActive: false },
    });
  }
}
